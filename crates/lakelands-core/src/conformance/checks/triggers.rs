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

use std::sync::Arc;

use lakelands_api::event::EventBody;
use lakelands_api::expr::Condition;
use lakelands_api::ontology::{StorageKind, TriggerDecl};
use lakelands_api::principal::{WriteContext, WriteOrigin};
use lakelands_api::seam::FeedFilter;
use lakelands_api::write::Declaration;
use lakelands_api::{Find, LakeStore, Position, Refusal, TypeName, WriteOpts};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Must, declare, doors, field, head_unmoved, invalid_naming, limits, owner, patch, reason,
    type_decl, upsert,
};

check!(
    /// A transition guard fires on its transition only.
    TRANSITION = a_transition_guard_fires_on_open_to_closed_only,
    "004, extended: a guard `previous(status) = 'open' and status = 'closed'` fires on open → closed and not on draft → closed, open → open or a record created closed (`previous` is null for it); `previous` outside a guard is refused as invalid naming it"
);

check!(
    /// A guard tells a retirement from an update.
    RETIREMENT = a_guard_tells_a_retirement_from_an_update,
    "004, extended: a guard `status = 'closed' and not retired()` fires on closing a record and on updating a closed one but not on retiring it, and a guard `retired()` fires on the retirement only; the declaration calling `retired` is written in log format 7"
);

check!(
    /// A revival reads as a creation in a guard.
    REVIVAL = a_revival_is_a_creation_to_a_guard,
    "004, extended: an upsert of a retired record is a creation to a guard: `previous(f)` is null, `changed(f)` is true and `retired()` is false"
);

check!(
    /// `retired()` belongs to a trigger's guard alone.
    RETIRED_ONLY_IN_A_GUARD = retired_outside_a_triggers_guard_is_refused_naming_it,
    "004, extended: `retired()` in an object set's filter, a verb's `requires`, a derived field or a find's filter, and `retired` called with an argument in a guard, are refused as invalid naming `retired`, appending nothing"
);

check!(
    /// A reaction past its trigger's ceiling is refused.
    PER_TRIGGER_DEPTH = a_reaction_past_its_triggers_max_depth_is_refused_naming_it,
    "021, extended: a reactive write whose `cause` names a firing of a trigger with `max_depth` is refused too_deep naming the trigger when it is deeper than that ceiling, appending nothing, while the lake's own ceiling is higher; a reaction to another trigger, or naming no cause, at the same depth is taken"
);

check!(
    /// A trigger's ceiling lies within the lake's.
    TRIGGER_CEILING = a_trigger_max_depth_above_the_lakes_ceiling_is_refused,
    "021, extended: a trigger whose `max_depth` is above the lake's `max_reactive_depth` is refused as invalid naming the trigger and both numbers; the lake's ceiling itself and 0 are taken"
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
    Ok(store
        .feed()
        .poll(after.next(), &FeedFilter::default(), 10_000)
        .await
        .must("reading the feed")?
        .events
        .into_iter()
        .filter(|e| matches!(&e.body, EventBody::TriggerFired { trigger, .. } if trigger.as_str() == name))
        .map(|e| e.position)
        .collect())
}

async fn a_transition_guard_fires_on_open_to_closed_only(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    declare(
        cat.as_ref(),
        &own,
        type_decl("job", vec![field("status", StorageKind::Text)]),
    )
    .await?;
    cat.declare(
        &own,
        trigger(
            "closed",
            Some("previous(status) = 'open' and status = 'closed'"),
            None,
        ),
    )
    .await
    .must("declaring the transition trigger")?;
    for (key, states) in [
        ("a", vec!["open", "closed"]),
        ("b", vec!["draft", "closed"]),
        ("c", vec!["open", "open"]),
        ("d", vec!["closed"]),
    ] {
        let mut fires = Vec::new();
        for status in &states {
            let before = store.head().await.must("reading the head")?;
            upsert(cat.as_ref(), &own, "job", key, json!({"status": status})).await?;
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
    invalid_naming(
        "`previous` outside a guard",
        cat.find(
            &own,
            &TypeName::from("job"),
            Find {
                filter: Some(Condition::from("previous(status) = 'open'")),
                ..Find::default()
            },
        )
        .await,
        &["previous"],
    )?;
    Ok(())
}

async fn a_guard_tells_a_retirement_from_an_update(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    declare(
        cat.as_ref(),
        &own,
        type_decl(
            "job",
            vec![
                field("status", StorageKind::Text),
                field("note", StorageKind::Text),
            ],
        ),
    )
    .await?;
    let before = store.head().await.must("reading the head")?;
    for d in [
        trigger("closed", Some("status = 'closed' and not retired()"), None),
        trigger("retired", Some("retired()"), None),
    ] {
        cat.declare(&own, d).await.must("declaring the triggers")?;
    }
    let declared = store
        .feed()
        .poll(before.next(), &FeedFilter::default(), 100)
        .await
        .must("reading the feed")?
        .events
        .into_iter()
        .find(|e| {
            matches!(&e.body, EventBody::Declared { declaration: Declaration::Trigger(t), .. }
                if t.name.as_str() == "retired")
        })
        .ok_or_else(|| Failure::new("the feed must hold the trigger `retired` declared"))?;
    ensure!(
        declared.v == 7,
        "a declaration calling `retired` is written in log format 7; it was written in {}",
        declared.v
    );
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
        upsert(cat.as_ref(), &own, "job", "a", write).await?;
        let got = fires(store.as_ref(), at, &names).await?;
        ensure!(
            got == want,
            "{step} must fire {names:?} as {want:?}; it fired {got:?}"
        );
    }
    let at = store.head().await.must("reading the head")?;
    cat.retire(
        &own,
        &TypeName::from("job"),
        &"a".into(),
        WriteOpts::default(),
    )
    .await
    .must("retiring `a`")?;
    let got = fires(store.as_ref(), at, &names).await?;
    ensure!(
        got == [false, true],
        "retiring the closed `a` must fire `retired` and not `closed`, whose fields still hold; \
         it fired {names:?} as {got:?}"
    );
    Ok(())
}

async fn a_revival_is_a_creation_to_a_guard(store: Arc<dyn LakeStore>) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    declare(
        cat.as_ref(),
        &own,
        type_decl("job", vec![field("status", StorageKind::Text)]),
    )
    .await?;
    cat.declare(
        &own,
        trigger(
            "created",
            Some("previous(status) is null and changed(status) and not retired()"),
            None,
        ),
    )
    .await
    .must("declaring the trigger")?;
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
    upsert(cat.as_ref(), &own, "job", "r", json!({"status": "open"})).await?;
    fired("creating `r`", true, at).await?;
    let at = store.head().await.must("reading the head")?;
    upsert(cat.as_ref(), &own, "job", "r", json!({"status": "open"})).await?;
    fired("writing `r` again", false, at).await?;
    let at = store.head().await.must("reading the head")?;
    cat.retire(
        &own,
        &TypeName::from("job"),
        &"r".into(),
        WriteOpts::default(),
    )
    .await
    .must("retiring `r`")?;
    fired("retiring `r`", false, at).await?;
    let at = store.head().await.must("reading the head")?;
    upsert(cat.as_ref(), &own, "job", "r", json!({"status": "open"})).await?;
    fired(
        "reviving `r` (`previous` null, `changed` true, `retired()` false)",
        true,
        at,
    )
    .await
}

async fn retired_outside_a_triggers_guard_is_refused_naming_it(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    declare(
        cat.as_ref(),
        &own,
        type_decl("job", vec![field("status", StorageKind::Text)]),
    )
    .await?;
    for (what, d) in [
        (
            "an object set filtering on `retired()`",
            json!({"declare": "object_set", "name": "gone", "of": "job", "where": "retired()"}),
        ),
        (
            "a verb requiring `not retired()`",
            json!({"declare": "verb", "name": "close", "on": "job", "risk": "low",
                   "requires": "not retired()", "effect": "write",
                   "set": [{"field": "status", "value": "'closed'"}]}),
        ),
        (
            "a field derived from `retired()`",
            json!({"declare": "type", "name": "task", "fields": [
                {"name": "gone", "type": {"kind": "boolean"}, "authority": "derived",
                 "derived": "retired()"}]}),
        ),
        (
            "a guard calling `retired(status)`",
            json!({"declare": "trigger", "name": "t", "on": "job", "when": "retired(status)"}),
        ),
    ] {
        let d: Declaration = serde_json::from_value(d).expect("a declaration");
        let before = store.head().await.must("reading the head")?;
        invalid_naming(what, cat.declare(&own, d).await, &["retired"])?;
        head_unmoved(store.as_ref(), before, what).await?;
    }
    invalid_naming(
        "a find filtering on `retired()`",
        cat.find(
            &own,
            &TypeName::from("job"),
            Find {
                filter: Some(Condition::from("retired()")),
                ..Find::default()
            },
        )
        .await,
        &["retired"],
    )
}

fn host(depth: u32, cause: Option<Position>) -> WriteContext {
    WriteContext {
        origin: WriteOrigin::Reactive,
        depth,
        cause,
    }
}

async fn a_reaction_past_its_triggers_max_depth_is_refused_naming_it(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    declare(
        cat.as_ref(),
        &own,
        type_decl("job", vec![field("n", StorageKind::Number)]),
    )
    .await?;
    for d in [
        trigger("once", None, Some(1)),
        trigger("always", None, None),
    ] {
        cat.declare(&own, d).await.must("declaring the triggers")?;
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
            store
                .catalog(ctx)
                .upsert(
                    &own,
                    &TypeName::from("job"),
                    &key.into(),
                    patch(json!({"n": 1})),
                    WriteOpts::default(),
                )
                .await
        }
    };
    // The direct write fires both triggers at depth 0.
    let head = store.head().await.must("reading the head")?;
    upsert(cat.as_ref(), &own, "job", "j0", json!({"n": 0})).await?;
    let once_0 = firing(head, "once").await?;
    // The reaction to it at depth 1 is within `once`'s ceiling, and fires
    // `once` again at depth 1.
    let head = store.head().await.must("reading the head")?;
    write(host(1, Some(once_0)), "j1")
        .await
        .must("a depth-1 reaction naming `once`'s firing")?;
    let once_1 = firing(head, "once").await?;
    let always_1 = firing(head, "always").await?;
    // A reaction to that firing at depth 2 passes `once`'s ceiling.
    let head = store.head().await.must("reading the head")?;
    match write(host(2, Some(once_1)), "j2").await {
        Err(e) if matches!(reason(&e), Some(Refusal::TooDeep { max: 1 })) => {
            let why = e.to_string();
            ensure!(
                why.contains("once"),
                "a reaction past `once`'s ceiling must be refused naming it; it said: {why}"
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
    write(host(2, Some(always_1)), "j3")
        .await
        .must("a depth-2 reaction naming `always`'s firing")?;
    write(host(2, None), "j4")
        .await
        .must("a depth-2 reaction naming no cause")?;
    Ok(())
}

async fn a_trigger_max_depth_above_the_lakes_ceiling_is_refused(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    declare(
        cat.as_ref(),
        &own,
        type_decl("job", vec![field("n", StorageKind::Number)]),
    )
    .await?;
    let ceiling = limits().max_reactive_depth;
    let head = store.head().await.must("reading the head")?;
    invalid_naming(
        &format!("a max_depth of {} above the lake's {ceiling}", ceiling + 1),
        cat.declare(&own, trigger("deep", None, Some(ceiling + 1)))
            .await,
        &["deep", &(ceiling + 1).to_string(), &ceiling.to_string()],
    )?;
    head_unmoved(store.as_ref(), head, "a trigger above the ceiling").await?;
    for (name, m) in [("at_ceiling", ceiling), ("never", 0)] {
        cat.declare(&own, trigger(name, None, Some(m)))
            .await
            .must(&format!("declaring a trigger with max_depth {m}"))?;
    }
    Ok(())
}
