//! What a trigger's guard sees, and how deep the reactions to it may go
//! (`docs/decisions/004-the-change-feed-is-the-log.md` and
//! `docs/decisions/021-the-write-context-belongs-to-the-host.md`, each
//! extended).
//!
//! A guard reads the write through `previous(f)` and `changed(f)`, so a
//! transition is a guard, and through `retired()`, so a retirement, which
//! keeps every field, is told from an update. A revival reads as a
//! creation. A trigger may cap the depth of the reactions that name its
//! firing as their cause, below the lake's own ceiling.

use lakelands_api::error::Case;
use lakelands_api::event::EventBody;
use lakelands_api::expr::Condition;
use lakelands_api::location::Limits;
use lakelands_api::ontology::{StorageKind, TriggerDecl};
use lakelands_api::principal::WriteContext;
use lakelands_api::seam::Seams;
use lakelands_api::write::{Declaration, ProposedWrite};
use lakelands_api::{LakeStore, Outcome, Position, Refusal, TypeName};
use serde_json::json;

use crate::conformance::fixture::{
    Lake, Must, events_after, field, find_where, head_unmoved, limits, reactive, reason,
    refused_like, refused_on, try_upsert, type_decl,
};
use crate::conformance::{Failure, Given, Held, Store};

check!(
    TRANSITION = a_transition_guard_fires_on_open_to_closed_only,
    "004, extended: a guard `previous(status) = 'open' and status = 'closed'` fires on open → closed and not on draft → closed, open → open or a record created closed (`previous` is null for it); `previous` outside a guard is refused as invalid naming it"
);

check!(
    RETIREMENT = a_guard_tells_a_retirement_from_an_update,
    "004, extended: a guard `status = 'closed' and not retired()` fires on closing a record and on updating a closed one but not on retiring it, and a guard `retired()` fires on the retirement only, caused by it, as it does on a proposed retirement when it is accepted"
);

check!(
    REVIVAL = a_revival_is_a_creation_to_a_guard,
    "004, extended: an upsert of a retired record is a creation to a guard: `previous(f)` is null, `changed(f)` is true and `retired()` is false"
);

check!(
    RETIRED_ONLY_IN_A_GUARD = retired_outside_a_triggers_guard_is_refused_naming_it,
    "004, extended: `retired()` in an object set's filter, a verb's `requires`, a derived field or a find's filter, and `retired` called with an argument in a guard, are refused as invalid naming `retired`, appending nothing"
);

check!(
    PER_TRIGGER_DEPTH = a_reaction_past_its_triggers_max_depth_is_refused_naming_it,
    "021, extended: a reactive write whose `cause` names a firing of a trigger with `max_depth` is refused too_deep naming the trigger when it is deeper than that ceiling, appending nothing, while the lake's own ceiling is higher; a reaction to another trigger, or naming no cause, at the same depth is taken"
);

check!(
    TRIGGER_CEILING = a_trigger_max_depth_above_the_lakes_ceiling_is_refused,
    "021, extended: a trigger whose `max_depth` is above the lake's `max_reactive_depth` is refused as invalid naming the trigger and both numbers, by `declare`, `apply` and `propose`, and by `accept` once the lake is opened again with a lower ceiling; the lake's ceiling itself and 0 are taken",
    given
);

fn trigger(name: &str, when: Option<&str>, max_depth: Option<u32>) -> Declaration {
    Declaration::Trigger(TriggerDecl {
        name: name.into(),
        on: "job".into(),
        when: when.map(Condition::from),
        description: None,
        max_depth,
    })
}

/// Whether one write fires each of `names`, in that order.
async fn fires(
    store: &dyn LakeStore,
    before: Position,
    names: &[&str],
) -> Result<Vec<bool>, Failure> {
    let mut out = Vec::new();
    for name in names {
        out.push(!fired_after(store, before, name).await?.is_empty());
    }
    Ok(out)
}

async fn fired_after(
    store: &dyn LakeStore,
    after: Position,
    name: &str,
) -> Result<Vec<Position>, Failure> {
    Ok(events_after(store, after)
        .await?
        .into_iter()
        .filter(|e| matches!(&e.body, EventBody::TriggerFired { trigger, .. } if trigger.as_str() == name))
        .map(|e| e.position)
        .collect())
}

async fn a_transition_guard_fires_on_open_to_closed_only(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    l.declare(type_decl("job", vec![field("status", StorageKind::Text)]))
        .await?;
    let fut = cat.declare(
        own,
        trigger(
            "closed",
            Some("previous(status) = 'open' and status = 'closed'"),
            None,
        ),
    );
    fut.await.must("declaring the transition trigger")?;
    for (key, states) in [
        ("a", vec!["open", "closed"]),
        ("b", vec!["draft", "closed"]),
        ("c", vec!["open", "open"]),
        ("d", vec!["closed"]),
    ] {
        let mut fires = Vec::new();
        for status in &states {
            let before = store.head().await.must("reading the head")?;
            l.upsert("job", key, json!({"status": status})).await?;
            fires.push(
                !fired_after(store.as_ref(), before, "closed")
                    .await?
                    .is_empty(),
            );
        }
        let want = key == "a";
        ensure!(
            fires.last() == Some(&want) && fires[..fires.len() - 1].iter().all(|f| !f),
            "`{key}` through {states:?} must fire the transition trigger {}; it fired {fires:?}",
            if want {
                "on its last write only"
            } else {
                "never"
            }
        );
    }
    refused_as!(
        "`previous` outside a guard",
        cat.find(
            own,
            &TypeName::from("job"),
            find_where("previous(status) = 'open'"),
        )
        .await,
        Case::OnlyInWrite { name } if name == "previous"
    )?;
    Ok(())
}

async fn a_guard_tells_a_retirement_from_an_update(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    l.declare(type_decl(
        "job",
        vec![
            field("status", StorageKind::Text),
            field("note", StorageKind::Text),
        ],
    ))
    .await?;
    for d in [
        trigger("closed", Some("status = 'closed' and not retired()"), None),
        trigger("retired", Some("retired()"), None),
    ] {
        cat.declare(own, d).await.must("declaring the triggers")?;
    }
    let names = ["closed", "retired"];
    for (step, write, want) in [
        (
            "creating `a` open",
            json!({"status": "open"}),
            [false, false],
        ),
        ("closing `a`", json!({"status": "closed"}), [true, false]),
        (
            "updating the closed `a`",
            json!({"note": "filed"}),
            [true, false],
        ),
    ] {
        let at = store.head().await.must("reading the head")?;
        l.upsert("job", "a", write).await?;
        let got = fires(store.as_ref(), at, &names).await?;
        ensure!(
            got == want,
            "{step} must fire {names:?} as {want:?}; it fired {got:?}"
        );
    }
    let at = store.head().await.must("reading the head")?;
    l.retire("job", "a").await?;
    let got = fires(store.as_ref(), at, &names).await?;
    ensure!(
        got == [false, true],
        "retiring the closed `a` must fire `retired` and not `closed`, whose fields still hold; \
         it fired {names:?} as {got:?}"
    );

    // A retirement that was proposed reacts when it is accepted.
    l.upsert("job", "b", json!({"status": "closed"})).await?;
    let write = ProposedWrite::Retire {
        ty: "job".into(),
        key: "b".into(),
        opts: Default::default(),
    };
    let proposed = cat.propose(own, write, "done".into()).await;
    let Outcome::Proposed { proposal, .. } = proposed.must("proposing to retire `b`")? else {
        return Err(Failure::new("a proposed retirement answers `proposed`"));
    };
    let at = store.head().await.must("reading the head")?;
    let fut = cat.accept(own, &proposal);
    fut.await.must("accepting the retirement of `b`")?;
    let events = events_after(store.as_ref(), at).await?;
    let retired = events
        .iter()
        .find(|e| matches!(e.body, EventBody::RecordRetired { .. }))
        .map(|e| e.position);
    let fired: Vec<_> = events
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::TriggerFired { trigger, .. } => Some((trigger.as_str(), e.cause)),
            _ => None,
        })
        .collect();
    ensure!(
        retired.is_some() && fired == [("retired", retired)],
        "accepting the proposed retirement of the closed `b` must retire it and fire `retired` \
         alone, caused by the retirement at {retired:?}; it fired {fired:?}"
    );
    Ok(())
}

async fn a_revival_is_a_creation_to_a_guard(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    l.declare(type_decl("job", vec![field("status", StorageKind::Text)]))
        .await?;
    let fut = cat.declare(
        own,
        trigger(
            "created",
            Some("previous(status) is null and changed(status) and not retired()"),
            None,
        ),
    );
    fut.await.must("declaring the trigger")?;
    let fired = |step: &'static str, want: bool, at: Position| {
        let store = store.clone();
        async move {
            let got = !fired_after(store.as_ref(), at, "created").await?.is_empty();
            ensure!(
                got == want,
                "{step} must {}fire a guard that holds on a creation",
                if want { "" } else { "not " }
            );
            Ok::<(), Failure>(())
        }
    };
    let at = store.head().await.must("reading the head")?;
    l.upsert("job", "r", json!({"status": "open"})).await?;
    fired("creating `r`", true, at).await?;
    let at = store.head().await.must("reading the head")?;
    l.upsert("job", "r", json!({"status": "open"})).await?;
    fired("writing `r` again", false, at).await?;
    let at = store.head().await.must("reading the head")?;
    l.retire("job", "r").await?;
    fired("retiring `r`", false, at).await?;
    let at = store.head().await.must("reading the head")?;
    l.upsert("job", "r", json!({"status": "open"})).await?;
    fired(
        "reviving `r` (`previous` null, `changed` true, `retired()` false)",
        true,
        at,
    )
    .await
}

async fn retired_outside_a_triggers_guard_is_refused_naming_it(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    l.declare(type_decl("job", vec![field("status", StorageKind::Text)]))
        .await?;
    let guard = json!({"code": "retired_only_in_guard"});
    for (what, d, case) in [
        (
            "an object set filtering on `retired()`",
            json!({"declare": "object_set", "name": "gone", "of": "job", "where": "retired()"}),
            &guard,
        ),
        (
            "a verb requiring `not retired()`",
            json!({"declare": "verb", "name": "close", "on": "job", "risk": "low",
                   "requires": "not retired()", "effect": "write",
                   "set": [{"field": "status", "value": "'closed'"}]}),
            &guard,
        ),
        (
            "a field derived from `retired()`",
            json!({"declare": "type", "name": "task", "fields": [
                {"name": "gone", "type": {"kind": "boolean"}, "authority": "derived",
                 "derived": "retired()"}]}),
            &guard,
        ),
        (
            "a guard calling `retired(status)`",
            json!({"declare": "trigger", "name": "t", "on": "job", "when": "retired(status)"}),
            &json!({"code": "not_parsed", "text": "retired(status)", "why":
                {"code": "parse_arity_exact", "name": "retired", "takes": 0, "given": 1}}),
        ),
    ] {
        let d: Declaration = serde_json::from_value(d).expect("a declaration");
        let before = store.head().await.must("reading the head")?;
        refused_like(what, cat.declare(own, d).await, case)?;
        head_unmoved(store.as_ref(), before, what).await?;
    }
    refused_as!(
        "a find filtering on `retired()`",
        cat.find(own, &TypeName::from("job"), find_where("retired()"))
            .await,
        Case::RetiredOnlyInGuard
    )
}

async fn a_reaction_past_its_triggers_max_depth_is_refused_naming_it(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    l.declare(type_decl("job", vec![field("n", StorageKind::Number)]))
        .await?;
    for d in [
        trigger("once", None, Some(1)),
        trigger("always", None, None),
    ] {
        cat.declare(own, d).await.must("declaring the triggers")?;
    }
    ensure!(
        limits().max_reactive_depth > 1,
        "the lake's ceiling must be above the trigger's 1 for the check to mean anything"
    );
    let firing = |after: Position, name: &'static str| {
        let store = store.clone();
        async move {
            fired_after(store.as_ref(), after, name)
                .await?
                .last()
                .copied()
                .ok_or_else(|| Failure::new(format!("the write must fire `{name}`")))
        }
    };
    let write = |ctx: WriteContext, key: &'static str| {
        let (store, own) = (store.clone(), own.clone());
        async move {
            let cat = store.catalog(ctx);
            try_upsert(cat.as_ref(), &own, "job", key, json!({"n": 1})).await
        }
    };
    // The direct write fires both triggers at depth 0.
    let head = store.head().await.must("reading the head")?;
    l.upsert("job", "j0", json!({"n": 0})).await?;
    let once_0 = firing(head, "once").await?;
    // The reaction to it at depth 1 is within `once`'s ceiling, and fires
    // `once` again at depth 1.
    let head = store.head().await.must("reading the head")?;
    write(reactive(1, Some(once_0)), "j1")
        .await
        .must("a depth-1 reaction naming `once`'s firing")?;
    let once_1 = firing(head, "once").await?;
    let always_1 = firing(head, "always").await?;
    // A reaction to that firing at depth 2 passes `once`'s ceiling.
    let head = store.head().await.must("reading the head")?;
    match write(reactive(2, Some(once_1)), "j2").await {
        Err(e) if matches!(reason(&e), Some(Refusal::TooDeep { max: 1 })) => {
            ensure!(
                matches!(refused_on(&e), Some(("once", _))),
                "a reaction past `once`'s ceiling must be refused naming it; it said: {e:?}"
            );
        }
        other => {
            return Err(Failure::new(format!(
                "a depth-2 reaction naming a firing of `once` (max_depth 1) must be refused \
                 too_deep with max 1; it answered {other:?}"
            )));
        }
    }
    head_unmoved(
        store.as_ref(),
        head,
        "a reaction past its trigger's ceiling",
    )
    .await?;
    write(reactive(2, Some(always_1)), "j3")
        .await
        .must("a depth-2 reaction naming `always`'s firing")?;
    let fut = write(reactive(2, None), "j4");
    fut.await.must("a depth-2 reaction naming no cause")?;
    Ok(())
}

async fn a_trigger_max_depth_above_the_lakes_ceiling_is_refused(
    store: Store,
    given: Given,
) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    l.declare(type_decl("job", vec![field("n", StorageKind::Number)]))
        .await?;
    let ceiling = limits().max_reactive_depth;
    let head = store.head().await.must("reading the head")?;
    let deep = || trigger("deep", None, Some(ceiling + 1));
    let named = json!({"code": "trigger_too_deep", "depth": ceiling + 1, "max": ceiling,
                       "at": [{"code": "trigger", "name": "deep"}]});
    for (door, answer) in [
        ("declare", cat.declare(own, deep()).await),
        ("apply", cat.apply(own, vec![deep()]).await),
        (
            "propose",
            cat.propose(
                own,
                ProposedWrite::Declare {
                    declarations: vec![deep()],
                },
                "deeper".into(),
            )
            .await,
        ),
    ] {
        let what = format!(
            "`{door}`: a max_depth of {} above the lake's {ceiling}",
            ceiling + 1
        );
        refused_like(&what, answer, &named)?;
    }
    head_unmoved(store.as_ref(), head, "a trigger above the ceiling").await?;
    let pending = cat
        .propose(
            own,
            ProposedWrite::Declare {
                declarations: vec![trigger("pending", None, Some(ceiling))],
            },
            "at the ceiling".into(),
        )
        .await
        .must("proposing a trigger with max_depth at the ceiling")?;
    let Outcome::Proposed { proposal, .. } = pending else {
        return Err(Failure::new(format!(
            "a proposal answers `proposed`; it answered {pending:?}"
        )));
    };
    for (name, m) in [("at_ceiling", ceiling), ("never", 0)] {
        cat.declare(own, trigger(name, None, Some(m)))
            .await
            .must(&format!("declaring a trigger with max_depth {m}"))?;
    }
    let lower = Limits {
        max_reactive_depth: ceiling - 1,
        ..limits()
    };
    let store = (given.relimit)(store, Seams::default(), lower)
        .await
        .map_err(|e| Failure::new(format!("opening the lake again with a lower ceiling: {e}")))?;
    let l = Lake::of(store.as_ref());
    refused_like(
        &format!(
            "accepting `pending` (max_depth {ceiling}) under a ceiling of {}",
            ceiling - 1
        ),
        l.cat.accept(&l.own, &proposal).await,
        &json!({"code": "trigger_too_deep", "depth": ceiling, "max": ceiling - 1,
                "at": [{"code": "trigger", "name": "pending"}]}),
    )
}
